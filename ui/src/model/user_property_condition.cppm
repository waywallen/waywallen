export module waywallen:model.user_property_condition;
import qextra;

export namespace waywallen::model
{

class UserPropertyCondition {
public:
    explicit UserPropertyCondition(const QString& expression = {}) {
        m_valid = compile_(expression);
        if (! m_valid) m_program.clear();
    }

    bool isValid() const { return m_valid; }

    bool matches(const QHash<QString, QJsonValue>& values) const {
        if (! m_valid || m_program.isEmpty()) return true;
        QList<QJsonValue> stack;
        for (const auto& instruction : m_program) {
            if (instruction.op == Op::Value) {
                stack.append(instruction.key.isEmpty()
                                 ? instruction.value
                                 : values.value(instruction.key, QJsonValue::Undefined));
            } else if (instruction.op == Op::Not) {
                if (stack.isEmpty()) return true;
                auto value = truth_(stack.takeLast());
                stack.append(value.isUndefined() ? value : QJsonValue(! value.toBool()));
            } else {
                if (stack.size() < 2) return true;
                auto right = stack.takeLast();
                auto left  = stack.takeLast();
                stack.append(binary_(instruction.op, left, right));
            }
        }
        if (stack.size() != 1) return true;
        auto result = truth_(stack.front());
        return result.isUndefined() || result.toBool();
    }

private:
    enum class Op
    {
        Open,
        Value,
        Not,
        AssignmentResult,
        Equal,
        NotEqual,
        StrictEqual,
        StrictNotEqual,
        Less,
        LessEqual,
        Greater,
        GreaterEqual,
        And,
        Or,
    };

    struct Instruction {
        Op         op;
        QJsonValue value;
        QString    key;
    };

    struct Operator {
        QStringView text;
        Op          op;
    };

    static int precedence_(Op op) {
        if (op == Op::Open) return 0;
        if (op == Op::Or) return 1;
        if (op == Op::And) return 2;
        if (op == Op::Not) return 5;
        if (op == Op::Less || op == Op::LessEqual || op == Op::Greater || op == Op::GreaterEqual)
            return 4;
        return 3;
    }

    static QJsonValue string_(const QString& quoted) {
        QString value;
        for (qsizetype i = 1; i + 1 < quoted.size(); ++i) {
            const auto character = quoted.at(i);
            if (character != u'\\') {
                value.append(character);
                continue;
            }
            if (++i + 1 >= quoted.size()) return QJsonValue::Undefined;
            switch (quoted.at(i).unicode()) {
            case '\\': value.append(u'\\'); break;
            case '\'': value.append(u'\''); break;
            case '"': value.append(u'"'); break;
            case '/': value.append(u'/'); break;
            case 'n': value.append(u'\n'); break;
            case 'r': value.append(u'\r'); break;
            case 't': value.append(u'\t'); break;
            case 'b': value.append(u'\b'); break;
            case 'f': value.append(u'\f'); break;
            case 'u': {
                if (i + 4 >= quoted.size() - 1) return QJsonValue::Undefined;
                bool valid = false;
                auto code  = quoted.mid(i + 1, 4).toUShort(&valid, 16);
                if (! valid) return QJsonValue::Undefined;
                value.append(QChar(code));
                i += 4;
                break;
            }
            default: return QJsonValue::Undefined;
            }
        }
        return value;
    }

    bool compile_(const QString& expression) {
        if (expression.size() > 4096) return false;
        if (expression.trimmed().isEmpty()) return true;
        static const QRegularExpression atom(QString::fromLatin1(
            R"expr((?:([\p{L}_$][\p{L}\p{N}_$]*)\s*\.\s*value\b)|(true\b|false\b|null\b)|([+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?)|("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'))expr"));
        static const QList<Operator>    operators {
            { QStringView(u"==="), Op::StrictEqual }, { QStringView(u"!=="), Op::StrictNotEqual },
            { QStringView(u"=="), Op::Equal },        { QStringView(u"!="), Op::NotEqual },
            { QStringView(u"<="), Op::LessEqual },    { QStringView(u">="), Op::GreaterEqual },
            { QStringView(u"&&"), Op::And },          { QStringView(u"||"), Op::Or },
            { QStringView(u"<"), Op::Less },          { QStringView(u">"), Op::Greater },
        };
        QList<Op> pending;
        bool      operand = true;
        qsizetype offset  = 0;
        while (offset < expression.size()) {
            if (m_program.size() + pending.size() > 512) return false;
            const auto character = expression.at(offset);
            if (character.isSpace()) {
                ++offset;
                continue;
            }
            if (operand) {
                const bool assigning_literal =
                    ! pending.isEmpty() && pending.back() == Op::AssignmentResult;
                if (character == u'!' || character == u'(') {
                    if (assigning_literal) return false;
                    pending.append(character == u'!' ? Op::Not : Op::Open);
                    ++offset;
                    continue;
                }
                auto match = atom.match(expression,
                                        offset,
                                        QRegularExpression::NormalMatch,
                                        QRegularExpression::AnchorAtOffsetMatchOption);
                if (! match.hasMatch()) return false;
                Instruction instruction { Op::Value, {}, match.captured(1) };
                if (assigning_literal &&
                    (! instruction.key.isEmpty() ||
                     ! QStringView(expression).sliced(match.capturedEnd()).trimmed().isEmpty()))
                    return false;
                if (! match.captured(2).isEmpty()) {
                    const auto literal = match.captured(2);
                    instruction.value  = literal == QLatin1String("null")
                                             ? QJsonValue::Null
                                             : QJsonValue(literal == QLatin1String("true"));
                } else if (! match.captured(3).isEmpty()) {
                    bool valid  = false;
                    auto number = match.captured(3).toDouble(&valid);
                    if (! valid || ! qIsFinite(number)) return false;
                    instruction.value = number;
                } else if (! match.captured(4).isEmpty()) {
                    instruction.value = string_(match.captured(4));
                    if (instruction.value.isUndefined()) return false;
                }
                m_program.append(instruction);
                offset  = match.capturedEnd();
                operand = false;
            } else if (character == u')') {
                while (! pending.isEmpty() && pending.back() != Op::Open)
                    m_program.append({ pending.takeLast(), {}, {} });
                if (pending.isEmpty()) return false;
                pending.removeLast();
                ++offset;
            } else {
                const auto remaining = QStringView(expression).sliced(offset);
                // Legacy assignments to a literal contribute only their result.
                if (character == u'=' && ! remaining.startsWith(QStringView(u"==")) &&
                    pending.isEmpty() && m_program.size() == 1 &&
                    ! m_program.front().key.isEmpty()) {
                    pending.append(Op::AssignmentResult);
                    ++offset;
                    operand = true;
                    continue;
                }
                bool found = false;
                for (const auto& pair : operators) {
                    if (remaining.startsWith(pair.text)) {
                        while (! pending.isEmpty() &&
                               precedence_(pending.back()) >= precedence_(pair.op))
                            m_program.append({ pending.takeLast(), {}, {} });
                        pending.append(pair.op);
                        offset += pair.text.size();
                        operand = true;
                        found   = true;
                        break;
                    }
                }
                if (! found) return false;
            }
        }
        if (operand) return false;
        while (! pending.isEmpty()) {
            if (pending.back() == Op::Open) return false;
            m_program.append({ pending.takeLast(), {}, {} });
        }
        return m_program.size() <= 512;
    }

    static QJsonValue truth_(const QJsonValue& value) {
        if (value.isUndefined()) return value;
        if (value.isNull()) return false;
        if (value.isBool()) return value;
        if (value.isDouble()) return value.toDouble() != 0.0;
        if (value.isString()) return ! value.toString().isEmpty();
        return true;
    }

    static double number_(const QJsonValue& value, bool& valid) {
        valid = true;
        if (value.isDouble()) return value.toDouble();
        if (value.isBool()) return value.toBool() ? 1.0 : 0.0;
        if (value.isNull()) return 0.0;
        if (value.isString()) {
            const auto text = value.toString().trimmed();
            if (text.isEmpty()) return 0.0;
            const auto number = text.toDouble(&valid);
            if (valid && qIsFinite(number)) return number;
        }
        valid = false;
        return 0.0;
    }

    static QJsonValue binary_(Op op, const QJsonValue& left, const QJsonValue& right) {
        if (op == Op::And || op == Op::Or) {
            auto a = truth_(left), b = truth_(right);
            if (op == Op::And &&
                ((! a.isUndefined() && ! a.toBool()) || (! b.isUndefined() && ! b.toBool())))
                return false;
            if (op == Op::Or &&
                ((! a.isUndefined() && a.toBool()) || (! b.isUndefined() && b.toBool())))
                return true;
            if (a.isUndefined() || b.isUndefined()) return QJsonValue::Undefined;
            return op == Op::And;
        }
        if (left.isUndefined() || right.isUndefined()) return QJsonValue::Undefined;
        if (op == Op::AssignmentResult) return right;
        if (op == Op::Equal || op == Op::NotEqual || op == Op::StrictEqual ||
            op == Op::StrictNotEqual) {
            bool equal = left == right;
            if (left.type() != right.type() && (op == Op::Equal || op == Op::NotEqual) &&
                ! left.isNull() && ! right.isNull()) {
                bool       a_valid = false, b_valid = false;
                const auto a = number_(left, a_valid), b = number_(right, b_valid);
                equal = a_valid && b_valid && a == b;
            }
            return op == Op::NotEqual || op == Op::StrictNotEqual ? ! equal : equal;
        }
        if (left.isString() && right.isString()) {
            const auto comparison = QString::compare(left.toString(), right.toString());
            if (op == Op::Less) return comparison < 0;
            if (op == Op::LessEqual) return comparison <= 0;
            if (op == Op::Greater) return comparison > 0;
            return comparison >= 0;
        }
        bool a_valid = false, b_valid = false;
        auto a = number_(left, a_valid), b = number_(right, b_valid);
        if (! a_valid || ! b_valid) return false;
        if (op == Op::Less) return a < b;
        if (op == Op::LessEqual) return a <= b;
        if (op == Op::Greater) return a > b;
        return a >= b;
    }

    QList<Instruction> m_program;
    bool               m_valid { true };
};

} // namespace waywallen::model
